// SPDX-License-Identifier: CC0-1.0

//! `PurchaseOrder$agreementId` as the restatement it is, end to end over HTTP.
//!
//! The rule under test: a `PurchaseOrder` is "information about a purchase order
//! association to a charge within an agreement", its `chargeId` is "the unique
//! identifier of the charge to associate the purchase order with" and its
//! optional `agreementId` is "the unique identifier of the agreement associated
//! with this charge" (`smithy_query(query="shape", shape="PurchaseOrder")`). The
//! agreement an association belongs to is therefore RESOLVED from the charge the
//! element names, and an element that states `agreementId` restates that
//! resolution: a value that is not the agreement holding that charge contradicts
//! the element's own `chargeId`, so it is refused in the service's modelled
//! `ValidationException` (HTTP 400) under the reason that names the member
//! (`INVALID_AGREEMENT_ID`) with `fields[0].name` at that element's own dotted
//! path, writing nothing — rather than being ignored or silently replaced. The
//! rule holds where this service performs the association,
//! `AcceptAgreementRequest$purchaseOrders`: at that door the agreement that will
//! hold the charges is minted BY the call, so no agreement holds them when the
//! request is made and every stated value is refused there.
//! `UpdatePurchaseOrders` performs no association at all
//! (`purchase_order_updates`), but it does require the member of every element it
//! is sent: a list whose every element states an agreement is refused there as
//! the unsupported action whatever the value — which the first test below pins,
//! because a refusal of the value there would tell a caller it could repair a
//! request that nothing will act on — while a list that leaves the member out
//! names no association to update and is refused on that member
//! (`purchase_order_update_agreement_id`).
//!
//! The rows differ in what the rule generalises over: whether the member is
//! stated at all, what it states (the holder, another agreement the store holds,
//! an id nothing holds, a one-character id, an id at `ResourceId`'s published
//! maximum length), how many agreements the call spans, how many charges each
//! holds, which charge an element names, how long the list is, the position of
//! the element at fault, and which operation carries the list.
//!
//! No agreement id or charge id is written by hand where the rule is about the
//! holder: both are read back from the call that minted them, so no row can agree
//! with a handler that invents ids. The associations are read back through
//! `ListAgreementCharges`, the service's own report of "charges and purchase
//! orders that are associated with an agreement".

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::agreement_lifecycle::{accept, emulator, Answer};
use crate::purchase_order_associations::{order, quote};
use crate::purchase_order_update_agreement_id::assert_missing_agreement_id;
use crate::purchase_order_updates::{
    agreement_holding, assert_unsupported_action, charges, items, references, update,
};

/// The `ValidationExceptionReason` the model publishes for an `agreementId` the
/// request may not state that value for
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const INVALID_AGREEMENT_ID: &str = "INVALID_AGREEMENT_ID";

/// The sentence this service's `ValidationException` states for that reason,
/// which names the member the request got wrong.
const INVALID_AGREEMENT_ID_MESSAGE: &str = "Provided agreement id is invalid.";

/// A well-formed `ResourceId` no agreement of the store is ever minted under.
const UNHELD_AGREEMENT_ID: &str = "agmt-ffffffffffffffff";

/// The published bounds of the member: `PurchaseOrder$agreementId` targets
/// `ResourceId`, `@length(min: 1, max: 64)` with pattern `^[A-Za-z0-9_/-]+$`
/// (`smithy_query(query="shape", shape="PurchaseOrder")`).
const RESOURCE_ID_MIN: usize = 1;
const RESOURCE_ID_MAX: usize = 64;

/// A value at each of those bounds: the shortest and the longest id a caller can
/// state at all. Neither names an agreement, so both are refused by the rule
/// under test rather than by the boundary validator — [`assert_published_bounds`]
/// is what keeps them at the bounds.
const SHORTEST_ID: &str = "a";
const LONGEST_ID: &str = "A012345678901234567890123456789012345678901234567890123456789012";

/// The two literals above are at the published bounds of the member they are
/// stated for, so a row that states one is a row at that bound.
fn assert_published_bounds() {
    assert_eq!(
        SHORTEST_ID.chars().count(),
        RESOURCE_ID_MIN,
        "case {SHORTEST_ID:?}: expected the member's minimum length {RESOURCE_ID_MIN}, got {}",
        SHORTEST_ID.chars().count()
    );
    assert_eq!(
        LONGEST_ID.chars().count(),
        RESOURCE_ID_MAX,
        "case {LONGEST_ID:?}: expected the member's maximum length {RESOURCE_ID_MAX}, got {}",
        LONGEST_ID.chars().count()
    );
}

/// What an element states as its `agreementId`.
#[derive(Clone, Copy, Debug)]
enum Stated {
    /// The member is left out of the element entirely.
    Omitted,
    /// The agreement holding the charge the element names.
    Holder,
    /// The agreement at this position of the row's agreements, which holds the
    /// element's charge only when it is the element's own.
    Agreement(usize),
    /// A value stated outright, which no agreement of the store is under.
    Literal(&'static str),
}

/// One element of a row's list: the agreement whose charges it reaches into, the
/// position of the charge it names among them, and what it states about the
/// agreement.
type Element = (usize, usize, Stated);

/// One row of the table: how many charges each agreement of the row holds, and
/// the elements of the list.
struct Row {
    case: &'static str,
    agreements: &'static [usize],
    elements: &'static [Element],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no agreement stated at all",
            agreements: &[1],
            elements: &[(0, 0, Stated::Omitted)],
        },
        Row {
            case: "the agreement holding the only charge",
            agreements: &[1],
            elements: &[(0, 0, Stated::Holder)],
        },
        Row {
            case: "the agreement holding each charge of several",
            agreements: &[3],
            elements: &[
                (0, 0, Stated::Holder),
                (0, 1, Stated::Holder),
                (0, 2, Stated::Holder),
            ],
        },
        Row {
            case: "each agreement holding its own charge, across two agreements",
            agreements: &[2, 2],
            elements: &[(0, 1, Stated::Holder), (1, 0, Stated::Holder)],
        },
        Row {
            case: "the holder stated on one element and omitted on the other",
            agreements: &[2],
            elements: &[(0, 0, Stated::Holder), (0, 1, Stated::Omitted)],
        },
        Row {
            case: "an agreement that does not hold the charge the element names",
            agreements: &[1, 1],
            elements: &[(0, 0, Stated::Agreement(1))],
        },
        Row {
            case: "the other agreement of the call on the second element of two",
            agreements: &[2, 2],
            elements: &[(0, 0, Stated::Holder), (1, 1, Stated::Agreement(0))],
        },
        Row {
            case: "an id no agreement of the store is under",
            agreements: &[1],
            elements: &[(0, 0, Stated::Literal(UNHELD_AGREEMENT_ID))],
        },
        Row {
            case: "an unheld id on the last element of three",
            agreements: &[3],
            elements: &[
                (0, 0, Stated::Holder),
                (0, 1, Stated::Omitted),
                (0, 2, Stated::Literal(UNHELD_AGREEMENT_ID)),
            ],
        },
        Row {
            case: "the shortest id the member admits",
            agreements: &[2],
            elements: &[(0, 1, Stated::Literal(SHORTEST_ID))],
        },
        Row {
            case: "the longest id the member admits",
            agreements: &[1],
            elements: &[(0, 0, Stated::Literal(LONGEST_ID))],
        },
        Row {
            case: "the longest id the member admits, on the second element of two",
            agreements: &[2],
            elements: &[(0, 0, Stated::Holder), (0, 1, Stated::Literal(LONGEST_ID))],
        },
    ]
}

/// The reference the element at `position` of a row's list associates.
fn reference(position: usize) -> String {
    format!("PO-{position}")
}

/// `order(charge_id, reference)` with `agreementId` stated as `value` — or left
/// out of the element entirely when `value` is `None`.
fn order_naming(charge_id: &str, reference: &str, value: Option<&str>) -> Value {
    let mut element = order(charge_id, reference);
    if let Some(value) = value {
        element["agreementId"] = json!(value);
    }
    element
}

/// Assert `answer` is the modelled refusal of the `agreementId` of the
/// `position`-th element of the request's list.
fn assert_refused(case: &str, answer: &Answer, position: usize) {
    let field = format!("purchaseOrders.{position}.member.agreementId");
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        INVALID_AGREEMENT_ID,
        "case {case:?}: expected reason {INVALID_AGREEMENT_ID:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/fields/0/name"),
        field,
        "case {case:?}: expected the refusal to name {field:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        INVALID_AGREEMENT_ID_MESSAGE,
        "case {case:?}: expected the message {INVALID_AGREEMENT_ID_MESSAGE:?}, got {:?}",
        answer.body
    );
}

/// `UpdatePurchaseOrders` associates nothing whatever its elements state about
/// the agreement: the VALUE is not what it refuses a list for, so every list
/// whose elements each state one — the one naming each charge's own holder among
/// them — is refused as the unsupported action, while one that leaves the member
/// out is refused on that member; either way the charge report is unchanged.
#[tokio::test]
async fn an_update_states_the_agreement_holding_the_charge() {
    assert_published_bounds();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();

        // The agreements the row asks for, each with its own charges.
        let mut agreements: Vec<(String, Vec<String>)> = Vec::new();
        for (position, held) in row.agreements.iter().enumerate() {
            agreements.push(
                agreement_holding(&router, &format!("ap-offerAI{index}x{position}"), *held).await,
            );
        }

        let orders: Vec<Value> = row
            .elements
            .iter()
            .enumerate()
            .map(|(position, (agreement, charge, stated))| {
                let (holder, charge_ids) = &agreements[*agreement];
                let value = match stated {
                    Stated::Omitted => None,
                    Stated::Holder => Some(holder.clone()),
                    Stated::Agreement(other) => Some(agreements[*other].0.clone()),
                    Stated::Literal(literal) => Some((*literal).to_string()),
                };
                order_naming(&charge_ids[*charge], &reference(position), value.as_deref())
            })
            .collect();

        let answered = update(&router, Value::Array(orders)).await;
        // An element stating no agreement names no association to update, so the
        // update door reports THAT member first; a list whose every element
        // states one is refused as the action, whatever the value.
        match row
            .elements
            .iter()
            .position(|(_, _, stated)| matches!(stated, Stated::Omitted))
        {
            Some(at) => assert_missing_agreement_id(case, &answered, at + 1),
            None => assert_unsupported_action(case, &answered),
        }

        // No charge of any agreement the row holds carries an association.
        for (position, (agreement, charge_ids)) in agreements.iter().enumerate() {
            let unassociated: BTreeMap<String, Option<String>> = charge_ids
                .iter()
                .map(|charge_id| (charge_id.clone(), None))
                .collect();
            let reported = references(&router, agreement).await;
            assert_eq!(
                reported, unassociated,
                "case {case:?}: expected the charges of agreement {position} to carry \
                 {unassociated:?}, got {reported:?}"
            );
        }
    }
}

/// The same rule on the list `AcceptAgreementRequest` carries: the charges that
/// call associates are held by no agreement when the request is made — the
/// agreement is minted by the call — so an element stating any `agreementId` at
/// all is refused, nothing is accepted, and a list stating none is accepted with
/// the association recorded under the agreement the call minted.
#[tokio::test]
async fn an_accepted_list_states_no_agreement_because_none_holds_its_charges_yet() {
    assert_published_bounds();
    let router = emulator();
    // An agreement the store already holds, so a row can state the id of an
    // agreement that EXISTS and is still refused: it does not hold the charges
    // the accepted quote is about to mint.
    let (existing, _) = agreement_holding(&router, "ap-offerAIexisting", 1).await;

    struct Accepted {
        case: &'static str,
        /// How many charges the quote mints.
        charges: usize,
        /// One element per purchase order: the position of the charge it names
        /// among the quote's charges, and what it states as its `agreementId` —
        /// `None` for the member left out, `Some(None)` for the id of the
        /// agreement the store already holds, and `Some(Some(literal))` for a
        /// value stated outright.
        #[allow(clippy::option_option)]
        elements: &'static [(usize, Option<Option<&'static str>>)],
        refused_at: Option<usize>,
    }
    let acceptances = [
        Accepted {
            case: "no agreement stated at all",
            charges: 1,
            elements: &[(0, None)],
            refused_at: None,
        },
        Accepted {
            case: "every element of a longer list stating none",
            charges: 3,
            elements: &[(0, None), (1, None), (2, None)],
            refused_at: None,
        },
        Accepted {
            case: "an id no agreement of the store is under",
            charges: 1,
            elements: &[(0, Some(Some(UNHELD_AGREEMENT_ID)))],
            refused_at: Some(1),
        },
        Accepted {
            case: "the id of an agreement the store already holds",
            charges: 2,
            elements: &[(0, None), (1, Some(None))],
            refused_at: Some(2),
        },
        Accepted {
            case: "the shortest id the member admits, on the first element of two",
            charges: 2,
            elements: &[(0, Some(Some(SHORTEST_ID))), (1, None)],
            refused_at: Some(1),
        },
        Accepted {
            case: "the longest id the member admits, on the last element of three",
            charges: 3,
            elements: &[(0, None), (1, None), (2, Some(Some(LONGEST_ID)))],
            refused_at: Some(3),
        },
    ];

    for (index, row) in acceptances.iter().enumerate() {
        let case = row.case;
        let (request_id, charge_ids) =
            quote(&router, &format!("ap-offerAIa{index}"), row.charges).await;
        let orders: Vec<Value> = row
            .elements
            .iter()
            .enumerate()
            .map(|(position, (charge, stated))| {
                let value = match stated {
                    None => None,
                    Some(None) => Some(existing.clone()),
                    Some(Some(literal)) => Some((*literal).to_string()),
                };
                order_naming(&charge_ids[*charge], &reference(position), value.as_deref())
            })
            .collect();

        let answered = accept(
            &router,
            json!({"agreementRequestId": request_id, "purchaseOrders": orders}),
        )
        .await;

        match row.refused_at {
            None => {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: accepting must succeed, got {} with {:?}",
                    answered.status, answered.body
                );
                // The agreement the call minted is the one the associations are
                // recorded under: the report states it on every charge of it.
                let minted = answered.str_at("/agreementId");
                let mut expected: BTreeMap<String, Option<String>> = charge_ids
                    .iter()
                    .map(|charge_id| (charge_id.clone(), None))
                    .collect();
                for (position, (charge, _)) in row.elements.iter().enumerate() {
                    expected.insert(charge_ids[*charge].clone(), Some(reference(position)));
                }
                let reported = references(&router, &minted).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the accepted agreement to carry {expected:?}, got \
                     {reported:?}"
                );
                let listed = charges(&router, json!({"agreementId": minted})).await;
                for charge in items(&listed) {
                    assert_eq!(
                        charge["agreementId"].as_str(),
                        Some(minted.as_str()),
                        "case {case:?}: expected charge {charge:?} to name agreement {minted:?}"
                    );
                }
            }
            Some(position) => {
                assert_refused(case, &answered, position);
                // Nothing was accepted: the quote is still acceptable, which is
                // only true if the refusal ran before anything was written.
                let retried = accept(&router, json!({"agreementRequestId": request_id})).await;
                assert_eq!(
                    retried.status, 200,
                    "case {case:?}: the refused quote must still be acceptable, got {} with {:?}",
                    retried.status, retried.body
                );
            }
        }
    }
}

/// The hinge of the rule, as one sequence: the id of the agreement an accept
/// mints is refused at the accept door of ANOTHER quote, because no agreement
/// holds that quote's charges — and once the accept has minted the holder, the
/// update door still takes no list for the charges it holds, because that
/// operation performs no association at all.
#[tokio::test]
async fn the_holder_of_a_charge_is_refused_at_the_accept_door_and_no_list_reaches_the_update_door() {
    let router = emulator();
    let (minted, minted_charges) = agreement_holding(&router, "ap-offerAIhinge", 2).await;

    // The update door takes no list, not even one naming the agreement that
    // holds the charge, and records nothing.
    let answered = update(
        &router,
        json!([order_naming(&minted_charges[0], "PO-held", Some(&minted))]),
    )
    .await;
    assert_unsupported_action("naming the agreement that holds the charge", &answered);
    let unassociated: BTreeMap<String, Option<String>> = minted_charges
        .iter()
        .map(|charge_id| (charge_id.clone(), None))
        .collect();
    let reported = references(&router, &minted).await;
    assert_eq!(
        reported, unassociated,
        "expected the charge report to carry {unassociated:?}, got {reported:?}"
    );

    // The accept door refuses the same id for the charges of a quote it does not
    // hold — which is every charge of every quote, since the holder of those
    // charges is minted by the accept itself.
    let (request_id, quoted_charges) = quote(&router, "ap-offerAIhinge2", 1).await;
    let refused = accept(
        &router,
        json!({
            "agreementRequestId": request_id,
            "purchaseOrders": [order_naming(&quoted_charges[0], "PO-not-yet", Some(&minted))],
        }),
    )
    .await;
    assert_refused("the holder of another agreement's charge", &refused, 1);

    // Accepted without the member, the same list records the association under
    // the agreement that call mints — and that is where the association this
    // service holds comes from.
    let accepted = accept(
        &router,
        json!({
            "agreementRequestId": request_id,
            "purchaseOrders": [order(&quoted_charges[0], "PO-now")],
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting without the member must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let holder = accepted.str_at("/agreementId");
    assert_ne!(
        holder, minted,
        "each accept mints its own agreement, got {holder:?} twice"
    );
    let after = references(&router, &holder).await;
    assert_eq!(
        after,
        BTreeMap::from([(quoted_charges[0].clone(), Some("PO-now".to_string()))]),
        "expected the accepted association on the charge, got {after:?}"
    );

    // And the update door leaves that association as the accept recorded it.
    let later = update(
        &router,
        json!([order_naming(&quoted_charges[0], "PO-later", Some(&holder))]),
    )
    .await;
    assert_unsupported_action("naming the agreement the accept minted", &later);
    let unchanged = references(&router, &holder).await;
    assert_eq!(
        unchanged, after,
        "expected the association the accept recorded to stand, got {unchanged:?}"
    );
}
