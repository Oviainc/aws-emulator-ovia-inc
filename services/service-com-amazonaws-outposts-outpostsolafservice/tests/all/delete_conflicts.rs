// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what makes a `DeleteOutpost` a CONFLICT rather than a delete,
//! driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses), starting
//! its capacity tasks through [`crate::capacity_task_lifecycle`]'s own
//! `StartCapacityTask` helper and its quotes through
//! [`crate::outpost_decommission`]'s.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. An Outpost with UNFINISHED WORK on it is not removed: while the store holds a
//!    capacity task of that Outpost in a non-terminal `CapacityTaskStatus`, or an
//!    order submitted for it that is still outstanding, `DeleteOutpost` answers 409
//!    `ConflictException` naming the resource causing the conflict in `ResourceId` /
//!    `ResourceType` — and the Outpost, the task and the order are all exactly as
//!    they were, so the caller can poll or cancel the work and delete the Outpost
//!    afterwards.
//! 2. Work in a TERMINAL status is finished work and holds nothing up: a capacity
//!    task in any terminal status, a quote no order was submitted from and an expired
//!    quote all leave the delete a 200 — as does work on ANOTHER Outpost.
//! 3. What belongs to the Outpost's own record is an attribute of it rather than work
//!    on it: a private connectivity configuration and the Amazon EC2 instances
//!    recorded as running on the Outpost go away WITH it and block nothing.
//!
//! Where the seeded state comes from: three of the modelled `CapacityTaskStatus`
//! values and the `ORDER_SUBMITTED` quote status are reached only through operations
//! that are still scaffold stubs (`CancelCapacityTask`, `CancelOrder`), so a row that
//! needs one puts the stored record in it through the same store seam
//! [`crate::outpost_decommission`] uses. Every assertion still goes over the wire.

use std::collections::VecDeque;

use serde_json::json;

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    CAPACITY_TASK_STATUS_VALUES, QUOTE_STATUS_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CAPACITY_TASK_REQUESTED, CAPACITY_TASK_TERMINAL, QUOTE_STATUS_ORDER_SUBMITTED,
    RESOURCE_TYPE_ORDER, RESOURCE_TYPE_OUTPOST,
};

use crate::blocking_instances::{record, running};
use crate::capacity_task_lifecycle::{pool, settle_capacity_task, start_ok};
use crate::outpost_decommission::{a_quote, set_quote_status};
use crate::site_and_outpost_lifecycle::{segment, text, Outposts, Reply};

/// An `OrderId` of the modelled form `^oo-[a-f0-9]{17}$` — the `SubmittedOrderId` a
/// quote an order was submitted from reports, and so the `ResourceId` a conflict an
/// order causes names.
const ORDER_ID: &str = "oo-3333333333333333c";

/// The wire error code a conflict arrives as, from
/// `smithy_query(query="shape",shape="ConflictException")`: `ConflictException` is
/// HTTP 409 and carries `ResourceId` / `ResourceType`, whose values come from the
/// modelled `ResourceType` enum (exactly `ORDER` and `OUTPOST`).
const CONFLICT_ERROR_CODE: &str = "ConflictException";

/// `DeleteOutpost` on either identifier form.
async fn delete(outposts: &Outposts, identifier: &str) -> Reply {
    outposts
        .call("DELETE", &format!("/outposts/{}", segment(identifier)), None)
        .await
}

/// `GetOutpost`, as the reply it answered: a row reads the Outpost back to show
/// whether the delete removed it.
async fn get_outpost(outposts: &Outposts, identifier: &str) -> Reply {
    outposts
        .call("GET", &format!("/outposts/{}", segment(identifier)), None)
        .await
}

/// `GetCapacityTask` on one task of one Outpost.
async fn get_task(outposts: &Outposts, outpost_id: &str, capacity_task_id: &str) -> Reply {
    outposts
        .call(
            "GET",
            &format!(
                "/outposts/{}/capacity/{}",
                segment(outpost_id),
                segment(capacity_task_id)
            ),
            None,
        )
        .await
}

/// `StartCapacityTask` on `outpost_id`, returning the minted `CapacityTaskId`.
async fn a_task(outposts: &Outposts, outpost_id: &str) -> String {
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });
    text(
        &start_ok(outposts, outpost_id, &request).await,
        "CapacityTaskId",
    )
}

/// Hold the stored capacity task in `status`, with no clock-owed transition left to
/// move it out again. See this module's header for why a row seeds a status.
fn hold_task_in(outposts: &Outposts, capacity_task_id: &str, status: &str, case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let task = store
        .capacity_tasks
        .get_mut(capacity_task_id)
        .unwrap_or_else(|| panic!("case {case:?}: the start persisted the task"));
    task.status = status.to_string();
    task.pending = VecDeque::new();
}

/// Record on the stored quote that an order WAS submitted from it: the modelled
/// `ORDER_SUBMITTED` status, plus the `SubmittedOrderId` naming the order when the
/// row records one.
fn submit_order_from(outposts: &Outposts, quote_id: &str, order_id: Option<&str>, case: &str) {
    set_quote_status(outposts, quote_id, QUOTE_STATUS_ORDER_SUBMITTED);
    let store = outposts.store();
    let mut store = store.lock();
    let quote = store
        .quotes
        .get_mut(quote_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted the quote"));
    quote.quote.submitted_order_id = order_id.map(str::to_string);
}

/// Assert that `reply` is the modelled conflict refusal naming `resource_type` /
/// `resource_id`, with a message that names each of `names` — the Outpost and the
/// work holding it, so the caller knows what to wait for. The status and the error
/// code are the requirement; the sentence itself is this emulator's own, so what is
/// asserted about it is that it names those resources, not its wording.
fn assert_conflict(
    reply: &Reply,
    case: &str,
    resource_type: &str,
    resource_id: &str,
    names: &[&str],
) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (409, CONFLICT_ERROR_CODE),
        "case {case:?}: expected HTTP 409 {CONFLICT_ERROR_CODE}, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.errortype_header, CONFLICT_ERROR_CODE,
        "case {case:?}: expected x-amzn-errortype {CONFLICT_ERROR_CODE}, got {:?}",
        reply.errortype_header
    );
    assert_eq!(
        text(&reply.body, "ResourceType"),
        resource_type,
        "case {case:?}: expected ResourceType {resource_type}, got {}",
        reply.body
    );
    assert_eq!(
        text(&reply.body, "ResourceId"),
        resource_id,
        "case {case:?}: expected ResourceId {resource_id}, got {}",
        reply.body
    );
    let message = text(&reply.body, "Message");
    for named in names {
        assert!(
            message.contains(named),
            "case {case:?}: expected the refusal to name {named}, got {message:?}"
        );
    }
}

/// Rules 1 and 2, over the status of the work: a capacity task of the Outpost in a
/// non-terminal status refuses the delete and survives it; one in a terminal status
/// holds nothing up, and goes with the Outpost.
///
/// The rows are EVERY value of the modelled `CapacityTaskStatus`
/// (`ops::types::CAPACITY_TASK_STATUS_VALUES`, read off the model's own enum, so the
/// set is not a copy this suite could drift from), which is what the rule
/// generalises over — including the statuses nothing in the implemented slice of the
/// API reaches. The identifier form the delete names the Outpost by alternates with
/// the row, since both forms name the same Outpost.
#[tokio::test]
async fn a_task_still_running_refuses_the_delete_and_a_finished_one_does_not() {
    for (index, status) in CAPACITY_TASK_STATUS_VALUES.iter().enumerate() {
        let case = format!("a capacity task of the Outpost held in {status}");
        // A store of its own per row: what blocks a delete is what the store holds.
        let outposts = Outposts::new();
        let (outpost_id, outpost_arn) = outposts.site_and_outpost("delete-conflict").await;
        let forms = [outpost_id.as_str(), outpost_arn.as_str()];
        let capacity_task_id = a_task(&outposts, &outpost_id).await;
        hold_task_in(&outposts, &capacity_task_id, status, &case);

        let finished = CAPACITY_TASK_TERMINAL.contains(status);
        let reply = delete(&outposts, forms[index % forms.len()]).await;
        if finished {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: finished work holds nothing up, expected HTTP 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_conflict(
                &reply,
                &case,
                RESOURCE_TYPE_OUTPOST,
                &outpost_id,
                &[outpost_id.as_str(), capacity_task_id.as_str()],
            );
        }

        // What the refusal left behind: a refused delete removed nothing, so the
        // Outpost and its task are both still readable; an accepted one took both.
        let expected_outpost = if finished { 404 } else { 200 };
        let outpost = get_outpost(&outposts, &outpost_id).await;
        assert_eq!(
            outpost.status, expected_outpost,
            "case {case:?}: expected GetOutpost {expected_outpost} after the delete, got {} {}",
            outpost.status, outpost.body
        );
        let task = get_task(&outposts, &outpost_id, &capacity_task_id).await;
        assert_eq!(
            task.status, expected_outpost,
            "case {case:?}: expected GetCapacityTask {expected_outpost} after the delete, got {} {}",
            task.status, task.body
        );
        if !finished {
            assert_eq!(
                text(&task.body, "CapacityTaskStatus"),
                *status,
                "case {case:?}: a refused delete must not move the task, got {}",
                task.body
            );
        }
    }
}

/// Rules 1 and 2, over the kind of work and whose Outpost it is: an order still
/// outstanding on the Outpost refuses the delete, and a quote no order was submitted
/// from — or one submitted for ANOTHER Outpost — does not.
///
/// The rows differ along the quote state the store holds: every value of the
/// modelled `QuoteStatus` (`ops::types::QUOTE_STATUS_VALUES`) on a quote of this
/// Outpost, with and without the `SubmittedOrderId` the submitted order is named by,
/// plus an order submitted for another Outpost, one for no Outpost at all, and no
/// quote whatsoever.
#[tokio::test]
async fn an_outstanding_order_refuses_the_delete_and_a_mere_quote_does_not() {
    /// Which Outpost the row's quote is associated with.
    enum Association {
        None,
        ThisOutpost,
        AnotherOutpost,
        NoOutpost,
    }
    /// What the row expects the delete to answer.
    enum Expect {
        Deleted,
        ConflictOnOrder(Option<&'static str>),
    }

    let mut cases: Vec<(String, Association, &str, Expect)> = vec![
        (
            "no quote at all".to_string(),
            Association::None,
            QUOTE_STATUS_ORDER_SUBMITTED,
            Expect::Deleted,
        ),
        (
            "an order submitted for ANOTHER Outpost".to_string(),
            Association::AnotherOutpost,
            QUOTE_STATUS_ORDER_SUBMITTED,
            Expect::Deleted,
        ),
        (
            "an order submitted from a quote of no Outpost".to_string(),
            Association::NoOutpost,
            QUOTE_STATUS_ORDER_SUBMITTED,
            Expect::Deleted,
        ),
        (
            "an order submitted for this Outpost, with no recorded order id".to_string(),
            Association::ThisOutpost,
            QUOTE_STATUS_ORDER_SUBMITTED,
            Expect::ConflictOnOrder(None),
        ),
    ];
    // Every modelled `QuoteStatus` on a quote of THIS Outpost: only the one that
    // says an order was submitted is work still outstanding.
    for status in QUOTE_STATUS_VALUES {
        let expect = match *status == QUOTE_STATUS_ORDER_SUBMITTED {
            true => Expect::ConflictOnOrder(Some(ORDER_ID)),
            false => Expect::Deleted,
        };
        cases.push((
            format!("a quote of this Outpost in {status}"),
            Association::ThisOutpost,
            status,
            expect,
        ));
    }

    for (case, association, quote_status, expect) in &cases {
        let outposts = Outposts::new();
        let (outpost_id, _) = outposts.site_and_outpost("delete-conflict").await;
        let (other_outpost_id, _) = outposts.site_and_outpost("another-outpost").await;
        let quote_id = match association {
            Association::None => None,
            Association::ThisOutpost => Some(a_quote(&outposts, Some(&outpost_id)).await),
            Association::AnotherOutpost => Some(a_quote(&outposts, Some(&other_outpost_id)).await),
            Association::NoOutpost => Some(a_quote(&outposts, None).await),
        };
        if let Some(quote_id) = &quote_id {
            match (quote_status == &QUOTE_STATUS_ORDER_SUBMITTED, expect) {
                (true, Expect::ConflictOnOrder(order_id)) => {
                    submit_order_from(&outposts, quote_id, *order_id, case)
                }
                (true, Expect::Deleted) => {
                    submit_order_from(&outposts, quote_id, Some(ORDER_ID), case)
                }
                (false, _) => set_quote_status(&outposts, quote_id, quote_status),
            }
        }

        let reply = delete(&outposts, &outpost_id).await;
        let expected_outpost = match expect {
            Expect::Deleted => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: nothing is outstanding on this Outpost, expected HTTP \
                     200, got {} {}",
                    reply.status, reply.body
                );
                404
            }
            Expect::ConflictOnOrder(Some(order_id)) => {
                assert_conflict(
                    &reply,
                    case,
                    RESOURCE_TYPE_ORDER,
                    order_id,
                    &[outpost_id.as_str(), order_id],
                );
                200
            }
            // A submitted order the store holds no id for still blocks the delete;
            // the only resource the caller can be given for it is the Outpost.
            Expect::ConflictOnOrder(None) => {
                let quote_id = quote_id
                    .as_deref()
                    .expect("a conflicting row always seeded a quote");
                assert_conflict(
                    &reply,
                    case,
                    RESOURCE_TYPE_OUTPOST,
                    &outpost_id,
                    &[outpost_id.as_str(), quote_id],
                );
                200
            }
        };
        let outpost = get_outpost(&outposts, &outpost_id).await;
        assert_eq!(
            outpost.status, expected_outpost,
            "case {case:?}: expected GetOutpost {expected_outpost} after the delete, got {} {}",
            outpost.status, outpost.body
        );
    }
}

/// Rule 3: what lives on the Outpost's own record is not work on the Outpost, so it
/// goes with the Outpost instead of blocking its delete.
///
/// The rows differ along which of those attributes the Outpost carries — a private
/// connectivity configuration, recorded running Amazon EC2 instances, both, or
/// neither.
#[tokio::test]
async fn an_outposts_own_attributes_do_not_block_its_delete() {
    // (case, carries a private connectivity configuration, carries instances)
    let cases: [(&str, bool, bool); 4] = [
        ("an Outpost carrying nothing", false, false),
        ("a private connectivity configuration", true, false),
        ("instances recorded as running on it", false, true),
        ("both", true, true),
    ];

    for (case, private_connectivity, instances) in &cases {
        let outposts = Outposts::new();
        let (outpost_id, _) = outposts.site_and_outpost("delete-conflict").await;
        if *private_connectivity {
            outposts
                .ok(
                    "POST",
                    &format!("/outposts/{outpost_id}/privateConnectivity"),
                    Some(&json!({ "VpcInformationList": [{ "VpcId": "vpc-0000000000001" }] })),
                )
                .await;
        }
        if *instances {
            // The instances [`crate::blocking_instances`] records, through that
            // suite's own store seam: an instance is launched through Amazon EC2 and
            // by no call in THIS API.
            record(&outposts, &outpost_id, &running(), case);
        }

        let reply = delete(&outposts, &outpost_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an attribute of the Outpost blocks nothing, expected HTTP 200, \
             got {} {}",
            reply.status, reply.body
        );
        let outpost = get_outpost(&outposts, &outpost_id).await;
        assert_eq!(
            outpost.status, 404,
            "case {case:?}: the delete removed the Outpost, expected GetOutpost 404, got {} {}",
            outpost.status, outpost.body
        );
    }
}

/// Rules 1 and 2 for an order the caller really placed: `CreateOrder` leaves an order
/// outstanding on the Outpost it names, so that Outpost's delete is refused 409
/// naming the ORDER — and the Outpost, the order and every other Outpost are left as
/// they were. An order placed for ANOTHER Outpost refuses nothing.
#[tokio::test]
async fn an_order_placed_on_the_outpost_refuses_the_delete() {
    // (case, whether the order is placed on the Outpost the row then deletes)
    let cases: [(&str, bool); 2] = [
        ("an order placed on this Outpost", true),
        ("an order placed on ANOTHER Outpost", false),
    ];

    for (case, on_this_outpost) in &cases {
        let outposts = Outposts::new();
        let (outpost_id, _) = outposts.site_and_outpost("delete-conflict").await;
        let (other_outpost_id, _) = outposts.site_and_outpost("another-outpost").await;
        let ordered_for = match on_this_outpost {
            true => &outpost_id,
            false => &other_outpost_id,
        };
        // The two members `CreateOrder` marks `@required`, plus one line item: a
        // `PaymentOption` of the modelled enum and a `CatalogItemId` of the modelled
        // `SkuCode` form (`^OR-[A-Z0-9]{7}$`).
        let order = outposts
            .ok(
                "POST",
                "/orders",
                Some(&json!({
                    "OutpostIdentifier": ordered_for,
                    "PaymentOption": "ALL_UPFRONT",
                    "LineItems": [{ "CatalogItemId": "OR-1A2B3C4", "Quantity": 1 }],
                })),
            )
            .await["Order"]
            .clone();
        let order_id = text(&order, "OrderId");

        let reply = delete(&outposts, &outpost_id).await;
        let expected_outpost = match on_this_outpost {
            true => {
                assert_conflict(
                    &reply,
                    case,
                    RESOURCE_TYPE_ORDER,
                    &order_id,
                    &[outpost_id.as_str(), order_id.as_str()],
                );
                200
            }
            false => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: another Outpost's order holds nothing up, expected HTTP \
                     200, got {} {}",
                    reply.status, reply.body
                );
                404
            }
        };
        let outpost = get_outpost(&outposts, &outpost_id).await;
        assert_eq!(
            outpost.status, expected_outpost,
            "case {case:?}: expected GetOutpost {expected_outpost} after the delete, got {} {}",
            outpost.status, outpost.body
        );
        // The order itself is untouched either way: a refused delete removed nothing,
        // and a delete of another Outpost never touched this order.
        let readback = outposts
            .call("GET", &format!("/orders/{}", segment(&order_id)), None)
            .await;
        assert_eq!(
            (readback.status, text(&readback.body["Order"], "OrderId")),
            (200, order_id.clone()),
            "case {case:?}: the order must still be readable, got {} {}",
            readback.status,
            readback.body
        );
    }
}

/// Rules 1 and 2 end to end with no seeded state at all: the capacity task the
/// caller just started holds the Outpost, and the delete the same caller retries
/// once the clock has finished the task goes through.
///
/// This is the whole of the refusal reached over the wire — `StartCapacityTask`,
/// `DeleteOutpost`, `GetCapacityTask` — over both identifier forms of the Outpost.
#[tokio::test]
async fn a_task_the_caller_just_started_holds_the_outpost_until_it_finishes() {
    for (case, form) in [("by bare id", 0usize), ("by Outpost ARN", 1usize)] {
        let outposts = Outposts::new();
        let (outpost_id, outpost_arn) = outposts.site_and_outpost("delete-conflict").await;
        let forms = [outpost_id.as_str(), outpost_arn.as_str()];
        let capacity_task_id = a_task(&outposts, &outpost_id).await;

        let refused = delete(&outposts, forms[form]).await;
        assert_conflict(
            &refused,
            case,
            RESOURCE_TYPE_OUTPOST,
            &outpost_id,
            &[outpost_id.as_str(), capacity_task_id.as_str()],
        );
        let held = get_task(&outposts, &outpost_id, &capacity_task_id).await;
        assert_eq!(
            (held.status, text(&held.body, "CapacityTaskStatus")),
            (200, CAPACITY_TASK_REQUESTED.to_string()),
            "case {case:?}: the refused delete left the task running, got {} {}",
            held.status,
            held.body
        );

        // The clock finishes the task; nothing else happens in between.
        settle_capacity_task();
        let accepted = delete(&outposts, forms[form]).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: once the task has finished the delete goes through, got {} {}",
            accepted.status, accepted.body
        );
        let gone = get_outpost(&outposts, &outpost_id).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the delete removed the Outpost, expected GetOutpost 404, got {} {}",
            gone.status, gone.body
        );
        let task = get_task(&outposts, &outpost_id, &capacity_task_id).await;
        assert_eq!(
            task.status, 404,
            "case {case:?}: the Outpost took its finished task with it, got {} {}",
            task.status, task.body
        );
    }
}
